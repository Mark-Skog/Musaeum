/**
 * The WebGL2 side of the compositor: one texture array holding every resident tile, and
 * one instanced draw call per frame. WebGL2 is the baseline because this is the renderer
 * Windows and macOS both have on an `OffscreenCanvas`; nothing here needs compute.
 */

import { TILE_SIZE } from '../protocol';
import type { QuadSink } from './frame-planner';
import { layersForBudget } from './tile-cache';

const VERTEX_SHADER = `#version 300 es
layout(location = 0) in vec4 a_destination;
layout(location = 1) in vec4 a_source;
layout(location = 2) in float a_layer;
uniform vec2 u_viewport;
out vec2 v_uv;
flat out float v_layer;
void main() {
  vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
  vec2 position = a_destination.xy + corner * a_destination.zw;
  v_uv = mix(a_source.xy, a_source.zw, corner);
  v_layer = a_layer;
  vec2 clip = position / u_viewport * 2.0 - 1.0;
  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
}`;

const FRAGMENT_SHADER = `#version 300 es
precision highp float;
precision highp sampler2DArray;
in vec2 v_uv;
flat in float v_layer;
uniform sampler2DArray u_tiles;
uniform vec3 u_paper;
out vec4 o_color;
void main() {
  // A negative layer marks a plate. Tiles are opaque, so alpha never matters here.
  o_color = v_layer < 0.0
    ? vec4(u_paper, 1.0)
    : vec4(texture(u_tiles, vec3(v_uv, v_layer)).rgb, 1.0);
}`;

const FLOATS_PER_QUAD = 9;
const MAX_QUADS = 8192;

/**
 * Marks over the pages: highlights, underlines, the selection. Each is any four-cornered shape,
 * since text can be turned on its page, with two colors: one for light paper and one for dark.
 *
 * The tiles of a frame are drawn into a texture first, and each mark reads the pixel under it
 * from that texture and decides by its luminance how to mix. Over light paper it takes the
 * minimum, as a highlighter does: white paper takes the color, black ink stays black, and two
 * marks over each other do not darken. Over a dark page it takes the maximum with the dark
 * color, the same thing the other way round: the dark paper takes a dimmer color and white type
 * stays white. In between, a smooth mix of the two. A mark with white as its light color and
 * black as its dark one changes nothing, which is how the first "mark" of every frame brings the
 * tiles from the texture onto the screen.
 */
const MARK_VERTEX_SHADER = `#version 300 es
layout(location = 0) in vec4 a_top;
layout(location = 1) in vec4 a_bottom;
layout(location = 2) in vec3 a_light;
layout(location = 3) in vec3 a_dark;
uniform vec2 u_viewport;
flat out vec3 v_light;
flat out vec3 v_dark;
void main() {
  // Corners come top left, top right, bottom right, bottom left; the strip wants the bottom ones
  // the other way round.
  vec2 position = gl_VertexID == 0 ? a_top.xy : gl_VertexID == 1 ? a_top.zw : gl_VertexID == 2 ? a_bottom.zw : a_bottom.xy;
  v_light = a_light;
  v_dark = a_dark;
  vec2 clip = position / u_viewport * 2.0 - 1.0;
  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
}`;

const MARK_FRAGMENT_SHADER = `#version 300 es
// highp, as the vertex shader has it: a uniform shared by both stages must agree on precision.
precision highp float;
flat in vec3 v_light;
flat in vec3 v_dark;
uniform sampler2D u_frame;
uniform vec2 u_viewport;
out vec4 o_color;
void main() {
  vec3 under = texture(u_frame, gl_FragCoord.xy / u_viewport).rgb;
  float luminance = dot(under, vec3(0.299, 0.587, 0.114));
  float light = smoothstep(0.30, 0.60, luminance);
  o_color = vec4(mix(max(under, v_dark), min(under, v_light), light), 1.0);
}`;

const FLOATS_PER_MARK = 14;
const MAX_MARKS = 16384;
/** The light and dark colors of the mark that only copies the frame. */
const COPY_LIGHT: readonly [number, number, number] = [1, 1, 1];
const COPY_DARK: readonly [number, number, number] = [0, 0, 0];

/** Matches the white the render worker fills tiles with, so plates never flash. */
const PAPER_COLOR: readonly [number, number, number] = [1, 1, 1];
const BACKDROP_COLOR: readonly [number, number, number] = [0.165, 0.165, 0.173];

export class TileRenderer implements QuadSink {
  private readonly gl: WebGL2RenderingContext;
  private readonly quads = new Float32Array(MAX_QUADS * FLOATS_PER_QUAD);
  private quadCount = 0;
  private viewportUniform: WebGLUniformLocation | null = null;
  private readonly marks = new Float32Array(MAX_MARKS * FLOATS_PER_MARK);
  /** The first mark of every frame is the copy of the tiles, so real marks start at 1. */
  private markCount = 1;
  /** Where the tiles of a frame are drawn, for the marks to read what is under them. */
  private frame: WebGLFramebuffer | null = null;
  private frameTexture: WebGLTexture | null = null;
  private frameWidth = 0;
  private frameHeight = 0;
  private tileProgram: WebGLProgram | null = null;
  private tileVertexArray: WebGLVertexArrayObject | null = null;
  private tileBuffer: WebGLBuffer | null = null;
  private markProgram: WebGLProgram | null = null;
  private markVertexArray: WebGLVertexArrayObject | null = null;
  private markBuffer: WebGLBuffer | null = null;
  private markViewportUniform: WebGLUniformLocation | null = null;
  /** Marks dropped because a frame had more than `MAX_MARKS`. Should stay at zero. */
  droppedMarks = 0;
  /** Quads dropped because the frame exceeded `MAX_QUADS`. Should stay at zero. */
  droppedQuads = 0;
  readonly gpuName: string;
  /** Layers in the texture array: as many as the budget pays for and the GPU allows. */
  readonly layers: number;

  constructor(
    private readonly canvas: OffscreenCanvas,
    budgetBytes: number,
    onContextLost: () => void,
    onContextRestored: () => void,
  ) {
    const gl = canvas.getContext('webgl2', {
      alpha: false,
      antialias: false,
      depth: false,
      stencil: false,
      powerPreference: 'high-performance',
      preserveDrawingBuffer: false,
    });
    if (gl === null) {
      throw new Error('WebGL2 is not available on OffscreenCanvas');
    }
    this.gl = gl;

    const maxLayers = gl.getParameter(gl.MAX_ARRAY_TEXTURE_LAYERS) as number;
    this.layers = layersForBudget(budgetBytes, maxLayers);
    const debug = gl.getExtension('WEBGL_debug_renderer_info');
    this.gpuName = debug
      ? (gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) as string)
      : (gl.getParameter(gl.RENDERER) as string);

    canvas.addEventListener('webglcontextlost', (event) => {
      event.preventDefault();
      onContextLost();
    });
    canvas.addEventListener('webglcontextrestored', () => {
      this.createResources();
      onContextRestored();
    });
    this.createResources();
  }

  /** Everything here is lost with the context and created again after a restore. */
  private createResources(): void {
    const { gl } = this;
    const program = linkProgram(gl, VERTEX_SHADER, FRAGMENT_SHADER);
    this.tileProgram = program;
    gl.useProgram(program);
    this.viewportUniform = gl.getUniformLocation(program, 'u_viewport');
    gl.uniform1i(gl.getUniformLocation(program, 'u_tiles'), 0);
    gl.uniform3fv(gl.getUniformLocation(program, 'u_paper'), PAPER_COLOR);

    const texture = gl.createTexture();
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, texture);
    gl.texStorage3D(gl.TEXTURE_2D_ARRAY, 1, gl.RGBA8, TILE_SIZE, TILE_SIZE, this.layers);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);

    const buffer = gl.createBuffer();
    this.tileBuffer = buffer;
    this.tileVertexArray = gl.createVertexArray();
    gl.bindVertexArray(this.tileVertexArray);
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, this.quads.byteLength, gl.DYNAMIC_DRAW);
    const stride = FLOATS_PER_QUAD * 4;
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 4, gl.FLOAT, false, stride, 0);
    gl.vertexAttribDivisor(0, 1);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, stride, 16);
    gl.vertexAttribDivisor(1, 1);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 1, gl.FLOAT, false, stride, 32);
    gl.vertexAttribDivisor(2, 1);

    const markProgram = linkProgram(gl, MARK_VERTEX_SHADER, MARK_FRAGMENT_SHADER);
    this.markProgram = markProgram;
    this.markViewportUniform = gl.getUniformLocation(markProgram, 'u_viewport');
    gl.useProgram(markProgram);
    gl.uniform1i(gl.getUniformLocation(markProgram, 'u_frame'), 1);
    gl.useProgram(program);
    // The frame texture, on unit 1; the tiles stay on unit 0, which `upload` relies on.
    this.frameTexture = gl.createTexture();
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.frameTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.activeTexture(gl.TEXTURE0);
    this.frame = gl.createFramebuffer();
    this.frameWidth = 0;
    this.frameHeight = 0;
    this.fitFrame();
    this.markBuffer = gl.createBuffer();
    this.markVertexArray = gl.createVertexArray();
    gl.bindVertexArray(this.markVertexArray);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.markBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, this.marks.byteLength, gl.DYNAMIC_DRAW);
    const markStride = FLOATS_PER_MARK * 4;
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 4, gl.FLOAT, false, markStride, 0);
    gl.vertexAttribDivisor(0, 1);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, markStride, 16);
    gl.vertexAttribDivisor(1, 1);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 3, gl.FLOAT, false, markStride, 32);
    gl.vertexAttribDivisor(2, 1);
    gl.enableVertexAttribArray(3);
    gl.vertexAttribPointer(3, 3, gl.FLOAT, false, markStride, 44);
    gl.vertexAttribDivisor(3, 1);
    gl.bindVertexArray(this.tileVertexArray);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.tileBuffer);
    gl.useProgram(program);

    gl.disable(gl.DEPTH_TEST);
    gl.disable(gl.BLEND);
    gl.clearColor(BACKDROP_COLOR[0], BACKDROP_COLOR[1], BACKDROP_COLOR[2], 1);
  }

  resize(width: number, height: number): void {
    this.canvas.width = width;
    this.canvas.height = height;
    this.gl.viewport(0, 0, width, height);
    this.fitFrame();
  }

  /** Gives the frame texture the canvas's size, when that changed. */
  private fitFrame(): void {
    const { gl } = this;
    const width = Math.max(1, this.canvas.width);
    const height = Math.max(1, this.canvas.height);
    if (this.frameTexture === null || (width === this.frameWidth && height === this.frameHeight)) {
      return;
    }
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.frameTexture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.frame);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.frameTexture, 0);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    this.frameWidth = width;
    this.frameHeight = height;
  }

  /** Copies one tile to the GPU. The array accepts both normal and shared buffers. */
  upload(layer: number, pixels: Uint8Array): void {
    const { gl } = this;
    gl.texSubImage3D(
      gl.TEXTURE_2D_ARRAY,
      0,
      0,
      0,
      layer,
      TILE_SIZE,
      TILE_SIZE,
      1,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      pixels as Uint8Array<ArrayBuffer>,
    );
  }

  plate(x: number, y: number, width: number, height: number): void {
    this.push(x, y, width, height, 0, 0, 0, 0, -1);
  }

  tile(
    x: number,
    y: number,
    width: number,
    height: number,
    u0: number,
    v0: number,
    u1: number,
    v1: number,
    layer: number,
  ): void {
    this.push(x, y, width, height, u0, v0, u1, v1, layer);
  }

  private push(
    x: number,
    y: number,
    width: number,
    height: number,
    u0: number,
    v0: number,
    u1: number,
    v1: number,
    layer: number,
  ): void {
    if (this.quadCount >= MAX_QUADS) {
      this.droppedQuads++;
      return;
    }
    const at = this.quadCount++ * FLOATS_PER_QUAD;
    const quads = this.quads;
    quads[at] = x;
    quads[at + 1] = y;
    quads[at + 2] = width;
    quads[at + 3] = height;
    quads[at + 4] = u0;
    quads[at + 5] = v0;
    quads[at + 6] = u1;
    quads[at + 7] = v1;
    quads[at + 8] = layer;
  }

  /**
   * A mark over the pages: four corners in device pixels (top left, top right, bottom right,
   * bottom left), the color it has over light paper and the one over a dark page, each channel
   * from 0 to 1. Drawn after every tile of the frame.
   */
  mark(corners: ArrayLike<number>, light: ArrayLike<number>, dark: ArrayLike<number>): void {
    if (this.markCount >= MAX_MARKS) {
      this.droppedMarks++;
      return;
    }
    this.write(this.markCount++, corners, light, dark);
  }

  private write(index: number, corners: ArrayLike<number>, light: ArrayLike<number>, dark: ArrayLike<number>): void {
    const at = index * FLOATS_PER_MARK;
    for (let corner = 0; corner < 8; corner++) {
      this.marks[at + corner] = corners[corner];
    }
    for (let channel = 0; channel < 3; channel++) {
      this.marks[at + 8 + channel] = light[channel];
      this.marks[at + 11 + channel] = dark[channel];
    }
  }

  /**
   * Draws everything pushed since the last call: the tiles in the order pushed, into the frame
   * texture; then the copy of that texture to the screen and the marks over it.
   */
  flush(): void {
    const { gl } = this;
    const width = this.canvas.width;
    const height = this.canvas.height;
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.frame);
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (this.quadCount > 0) {
      gl.uniform2f(this.viewportUniform, width, height);
      gl.bufferSubData(gl.ARRAY_BUFFER, 0, this.quads, 0, this.quadCount * FLOATS_PER_QUAD);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.quadCount);
    }
    this.quadCount = 0;
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    if (this.markProgram !== null) {
      this.write(0, [0, 0, width, 0, width, height, 0, height], COPY_LIGHT, COPY_DARK);
      gl.useProgram(this.markProgram);
      gl.bindVertexArray(this.markVertexArray);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.markBuffer);
      gl.uniform2f(this.markViewportUniform, width, height);
      gl.bufferSubData(gl.ARRAY_BUFFER, 0, this.marks, 0, this.markCount * FLOATS_PER_MARK);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.markCount);
      gl.useProgram(this.tileProgram);
      gl.bindVertexArray(this.tileVertexArray);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.tileBuffer);
    }
    this.markCount = 1;
  }
}

function compileShader(gl: WebGL2RenderingContext, type: GLenum, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (shader === null) {
    throw new Error('could not create a shader');
  }
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(`shader failed to compile: ${gl.getShaderInfoLog(shader)}`);
  }
  return shader;
}

function linkProgram(gl: WebGL2RenderingContext, vertex: string, fragment: string): WebGLProgram {
  const program = gl.createProgram();
  gl.attachShader(program, compileShader(gl, gl.VERTEX_SHADER, vertex));
  gl.attachShader(program, compileShader(gl, gl.FRAGMENT_SHADER, fragment));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    throw new Error(`shader program failed to link: ${gl.getProgramInfoLog(program)}`);
  }
  return program;
}
